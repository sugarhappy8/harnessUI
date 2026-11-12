from users import get_usr


def greeting(user_id):
    name = get_usr(user_id)
    if name is None:
        return "Hello, stranger"
    return f"Hello, {name}"
